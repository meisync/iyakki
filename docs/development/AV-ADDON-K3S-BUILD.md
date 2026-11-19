# Browser audio and video add-on: build it on the device in k3s (design, not built)

Status: design agreed in conversation 2026-11-06; nothing here is implemented. Related: [`AV-ADDON-BUILD.md`](AV-ADDON-BUILD.md), [`../FULL-BROWSER.md`](../FULL-BROWSER.md) ("Audio and video add-on").

## Why

IYAKKI hosts no FFmpeg, not even the archive (decision 2026-11-04). The Store's "Media player (ffmpeg)" is a static build, which has no shared libraries to load, so the browser add-on cannot use it. Vendor shared musl LGPL builds do not exist. Instead of asking the administrator to supply a package, the person's own machine builds it from the vendors' sources, at their click.

## Where it lives (decision 2026-11-06)

A separate public repository, `iyakki-media-bridge`, holds the bridge code (`gst-libav` glue and our patches), the build recipe and the **version map**: for every IYAKKI version, the GStreamer version and the FFmpeg version to build with, each with its source URL and SHA-256, the builder container (pinned by digest) and the exact `./configure` line (LGPL only: no `--enable-gpl`, no `--enable-nonfree`; h264, aac and mp3 decoders on). IYAKKI itself carries no FFmpeg.

* The entry for the image's own version is bundled in the image as a few lines of text (works offline). A newer entry is read from the repository as a small text download, after the person agrees.
* **Dead sources:** when a pinned source is no longer available, that map entry is removed. Those IYAKKI versions then have no build route and the person is told to use the mirror or file route. The repository's changelog announces a removal, with its date, before it happens.

## Flow

1. **Gate.** The Store entry is offered only when k3s is installed and running; otherwise it says so and points to the mirror or file route (which stay). The release's manifest must match the image's GStreamer version.
2. **Consent message** (the person must press a button; nothing starts before it): what is downloaded, from which hosts, the total size, that the libraries are built on this machine, the licence (LGPL-2.1+, build without GPL parts), the patent note for H.264 and AAC, the expected time and disk use.
3. **Isolated build pod:** unprivileged, no host mounts except one output folder, no service-account token, CPU, memory and disk limits, network allowed only to the manifest's hosts, a time limit.
4. **Checksums:** every download is checked against the manifest before it is used. A mismatch stops the build with the file name and both values.
5. **Output:** a package in the existing layout (`ffmpeg-libs-<version>-linux-musl-<arch>`, with NOTICE, licence files, SOURCE-OFFER and the patches), copied by the agent into `/local/packages` (administrators only; the copy checks that every path stays inside the folder), then through the existing review and confirm step.
6. **Stamp and notice:** the built package records the map entry and the image's GStreamer version. After an image update that changes either, the person gets an inbox notification "Browser audio and video needs a rebuild" and the engine starts without the add-on until they press Rebuild. Nothing is rebuilt on its own.

## Failure behaviour

Clear message per stage (gate, download, checksum, build, copy); a retry that resumes; the pod, the scratch folder and any partial output are removed on failure or Stop.

## Not covered by this design

* Machines without k3s or without a network (mirror or file route).
* GPL builds (refused by the existing check).
* Hardware decoding.

## Tests to write with it

Manifest parsing and pin checks; consent text contains the required lines; a checksum mismatch stops before building; the output copy cannot leave `/local/packages`; the stamp mismatch is reported; a QEMU run of the existing add-on test with a package built by the recipe.

## Open items

* A short legal review of the LGPL source-offer wording and the patent note before public release.
* Build time and memory on the Mac mini, measured.
* Which registry the builder image comes from, and whether it can be mirrored for offline organisations.
